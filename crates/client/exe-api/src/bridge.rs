//! Bridges over rivers: the viewer's team has them once it knows the technology with `BridgeBuilding` (Engineering).
//!
//! The original (Lekmod `CvTeam::changeBridgeBuildingCount`, CvTeam.cpp:3366, and the era change at 7581) tells the
//! renderer `gDLL->GameplayBridgeChanged(true, style)` when the ACTIVE team gains the technology, with style 1 (iron)
//! from the era `LAST_BRIDGE_ART_ERA` (Lekmod: 5, the Modern era) on and style 0 (stone) before; no other team's
//! bridges are told. So bridges are the viewer's one switch: a road over a river is drawn with a bridge only when the
//! viewer's team has the technology (a ford, i.e. the plain road, before), whoever owns the plot.
//!
//! The replica loads each turn's snapshot instead of running the turn (SIMP-exe-136), so the core's
//! `BRIDGE_CHANGED` event does not come when the technology is learnt; the state is read from the replica's Lua
//! (`Teams[Game.GetActiveTeam()]:IsBridgeBuilding()`) after each load and every [`POLL_FRAMES`] frames, and the host is
//! told when it changed ([`crate::ExeHost::bridge_changed`]).
//!
//! SIMPLIFIED(SIMP-exe-613): the state is polled from Lua (after a load, then every [`POLL_FRAMES`] frames) instead of
//! being raised by the core's `GameplayBridgeChanged` (not delivered in a replica that loads snapshots).

use lua_sys::Lua;

use crate::core::Core;

/// How often the state is read between loads (frames).
pub const POLL_FRAMES: u32 = 30;

/// `-1` unknown, else `bridge * 2 + style`.
const PROBE_LUA: &str = "\
local t = Game.GetActiveTeam()
local team = t and t >= 0 and Teams and Teams[t]
if not team then return -1 end
local last = GameDefines and GameDefines.LAST_BRIDGE_ART_ERA or 5
local bridge = team:IsBridgeBuilding() and 1 or 0
local style = (team:GetCurrentEra() >= last) and 1 or 0
return bridge * 2 + style";

/// The active team's bridge state: `(has bridges, iron)`; `None` when the game has no active team yet.
pub fn probe_bridge(lua: &Lua) -> Option<(bool, bool)> {
    let v = lua
        .load(PROBE_LUA)
        .set_name("=exe_bridge")
        .eval::<i32>()
        .map_err(|e| tracing::debug!(target: "exe.bridge", "the bridge probe failed: {e}"))
        .ok()?;
    (v >= 0).then_some((v / 2 == 1, v % 2 == 1))
}

impl Core {
    /// Once per frame (`ExeApi::dispatch`). `loaded`: the replica loaded a new state this frame.
    pub(crate) fn sync_bridge(&self, lua: &Lua, loaded: bool) {
        let tick = self.bridge_tick.get().wrapping_add(1);
        self.bridge_tick.set(tick);
        if !loaded && !tick.is_multiple_of(POLL_FRAMES) && self.bridge_seen.get().is_some() {
            return;
        }
        let Some(now) = probe_bridge(lua) else {
            return;
        };
        if self.bridge_seen.replace(Some(now)) != Some(now) {
            self.host.bridge_changed(now.0, now.1);
        }
    }
}
