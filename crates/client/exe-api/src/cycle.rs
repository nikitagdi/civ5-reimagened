//! The exe's automatic unit cycling: when the head selected unit of the seat can no longer be given an order (its moves are
//! used up, it fortified, slept, took a multi-turn move, or is gone) the exe selects the next unit that needs orders
//! (`CvGame::updateSelectionList` -> `cycleUnits(true)`), if the user option "auto unit cycle" is on.
//!
//! The exe's frame (`InterfaceBuddy::updateIfNotPaused` 0x086a6644, RB4-MP2 A4) counts the cycle counter down and calls
//! `updateSelectionList` at 0, but not while a popup is up or waiting, the city screen is up, the mode is PLACE_UNIT or a
//! city is selected. At the start of the turn the DLL arms the counter (`CvPlayer::setTurnActive`, CvPlayer.cpp:21057-21064)
//! when nothing is selected, so the first unit is selected on the turn's first free frame.
//!
//! SIMPLIFIED(SIMP-exe-590): the exe calls `updateSelectionList` from its own events (end of a mission, a command); the replica
//! has no such call, so it is read off the replica's state once per frame: the head unit that was ready for orders
//! (`IsReadyToMove` and not automated, i.e. `CvUnit::ReadyToSelect`) and is not any more, or is gone, makes the cycle. A
//! unit the player selects by hand while it is not ready (a fortified one) is not left; a selection the player cleared or
//! replaced is not cycled.
//!
//! SIMPLIFIED(SIMP-exe-634): the next unit is not the DLL's `Game.CycleUnits` (`CvGame::cycleUnits` -> `selectUnit` bumps the unit's
//! `m_iEverSelectedCount`, a part of the game the replica's digest checks: the replica diverged from the server at the first
//! batch). It is the seat's unit ready for orders nearest to the one left (ties: the lower id), selected through the
//! replica's own selection (`UI.SelectUnit`) and looked at; none left clears the selection. The exe's `CvUnitCycler` orders
//! the units as a chain of nearest neighbours from the head unit, which this follows only for the first step.
//!
//! SIMPLIFIED(SIMP-exe-631): the turn's start is the load of the turn's snapshot (or any load): on the first free frame after
//! it the next unit is selected when there is no head unit of the seat or it is not ready for orders (the exe arms the
//! counter only for an empty selection; here a unit kept selected across the load that has nothing to do is left too).
//!
//! SIMPLIFIED(SIMP-exe-632): while UI messages of the seat are out and not back in the journal
//! (`GameSide::own_orders_pending`, at most `replica::OWN_PENDING_MAX_MS`), the replica shows a unit just ordered as
//! still ready to move, so the cycle waits (the exe runs its own orders at once on a LAN and has no such wait): it neither
//! cycles nor takes a new head unit as "ready" until the echo is in. The popups, the city screen and PLACE_UNIT mode hold
//! it as in the exe; the exe's "city selection" (`isCitySelection`) does not, the client may keep a selected city after
//! the city screen closed.

use lua_sys::Lua;

use crate::core::Core;

/// The first part of a frame of the probe: true when the cycle can run this frame (the option is on, the seat's turn is
/// active and not ended). `__exe_cycle` is the unit the head selection was ready with (its id) and whether the turn's
/// first selection is due (`start`), kept across frames.
const AUTO_CYCLE_PRE_LUA: &str = "\
local me = Game.GetActivePlayer()
local p = me and me >= 0 and Players[me]
local st = __exe_cycle or {}
__exe_cycle = st
if __exe_cycle_reset then __exe_cycle_reset = nil st.id = nil st.start = true return false end
if not p or not OptionsManager.GetAutoUnitCycle() or not p:IsHuman() or not p:IsAlive() or not p:IsTurnActive() then st.id = nil return false end
if Network and Network.HasSentNetTurnComplete and Network.HasSentNetTurnComplete() then st.id = nil return false end
return true";

/// The rest of the frame, when nothing of the exe API holds the cycle. `cycle(from, x, y)` selects the seat's next unit
/// ready for orders (SIMP-exe-634), or clears a selection that has none left (`CvGame::cycleUnits`).
const AUTO_CYCLE_LUA: &str = "\
local me = Game.GetActivePlayer()
local p = Players[me]
local st = __exe_cycle
local function cycle(from, x, y)
	local best, bd
	for v in p:Units() do
		if v:GetID() ~= from and v:IsReadyToMove() and not v:IsAutomated() then
			local d = x and Map.PlotDistance(x, y, v:GetX(), v:GetY()) or 0
			if not best or d < bd or (d == bd and v:GetID() < best:GetID()) then best, bd = v, d end
		end
	end
	if best then
		UI.SelectUnit(best)
		if UI.LookAt and best.Plot then UI.LookAt(best:Plot()) end
	elseif from then
		UI.ClearSelectionList()
	end
end
if UI.GetInterfaceMode and InterfaceModeTypes and InterfaceModeTypes.INTERFACEMODE_PLACE_UNIT
	and UI.GetInterfaceMode() == InterfaceModeTypes.INTERFACEMODE_PLACE_UNIT then return end
local u = UI.GetHeadSelectedUnit()
if u and u:GetOwner() ~= me then u = nil end
local ready = u and u:IsReadyToMove() and not u:IsAutomated()
if st.start then
	st.start = nil
	if not ready then st.id = nil cycle(u and u:GetID(), u and u:GetX(), u and u:GetY()) return end
end
if u then
	if ready then st.id, st.x, st.y = u:GetID(), u:GetX(), u:GetY() return end
	if st.id == u:GetID() then st.id = nil cycle(u:GetID(), u:GetX(), u:GetY()) return end
	st.id = nil
	return
end
if st.id then
	local gone = p:GetUnitByID(st.id) == nil
	local id = st.id
	st.id = nil
	if gone then cycle(id, st.x, st.y) end
end";
impl Core {
    /// Once per frame (`ExeApi::dispatch`): the automatic unit cycling of the exe. `reset`: the replica loaded a new state
    /// (nothing is remembered across it; the turn's first selection is due).
    pub fn track_auto_cycle(&self, lua: &Lua, reset: bool) {
        if reset && let Err(e) = lua.load("__exe_cycle_reset = true").exec() {
            tracing::debug!(target: "exe.cycle", "reset failed: {e}");
        }
        match lua
            .load(AUTO_CYCLE_PRE_LUA)
            .set_name("=exe_auto_cycle_pre")
            .eval::<bool>()
        {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                tracing::debug!(target: "exe.cycle", "the auto cycle probe failed: {e}");
                return;
            }
        }
        if self.auto_cycle_held() {
            return;
        }
        if let Err(e) = lua.load(AUTO_CYCLE_LUA).set_name("=exe_auto_cycle").exec() {
            tracing::debug!(target: "exe.cycle", "the auto cycle probe failed: {e}");
        }
    }

    /// What holds the exe's cycle counter this frame: a popup up or waiting, the city screen, the seat's own orders on
    /// their way (SIMP-exe-632).
    fn auto_cycle_held(&self) -> bool {
        {
            let b = self.buddy.borrow();
            if b.is_popup_up() || b.waiting_len() > 0 {
                return true;
            }
        }
        if self.op(crate::UiOp::IsCityScreenUp, &[]).first() == Some(&1) {
            return true;
        }
        self.game()
            .and_then(|g| g.try_borrow().ok().map(|g| g.own_orders_pending()))
            .unwrap_or(false)
    }
}
