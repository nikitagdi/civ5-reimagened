//! `Steam`, `Matchmaking` and `InStrategicView`.
//!
//! `InStrategicView` is a function that answers `false` (U-D8: no strategic view). The EUI also decides
//! `civ5_mode = InStrategicView ~= nil`, so the global must exist. `StrategicViewStateChanged` is never
//! fired.
//!
//! `Matchmaking.IsHost`, `GetLocalID`, `GetHostID` answer from the seat of the lobby; the player list is
//! empty and the rest does nothing. `Steam.*` has no Steam to talk to.
//!
//! SIMPLIFIED(SIMP-exe-12): the stubs below.

use std::rc::Rc;

use lua_sys::{Lua, MultiValue, Result, Table};

use crate::Inner;
use crate::registry::{Reg, Stub};

/// Global functions of the exe that need no answer in the browser.
const GLOBAL_STUBS: &[(&str, Stub)] = &[
    ("IsGameCoreBusy", Stub::Bool(false)),
    ("UnitMoving", Stub::Bool(false)),
    ("ToggleStrategicView", Stub::Nil),
    ("MouseOverStrategicViewResource", Stub::Bool(false)),
    ("ProcessStrategicViewMouseClick", Stub::Bool(false)),
    ("GetGameViewRenderType", Stub::Num(0.0)),
    ("SetGameViewRenderType", Stub::Nil),
    ("StrategicViewShowFeatures", Stub::Nil),
    ("StrategicViewShowFogOfWar", Stub::Nil),
    ("getEndTurnTimerLength", Stub::Num(0.0)),
];

const STEAM: &[(&str, Stub)] = &[
    ("SaveGameToCloud", Stub::Nil),
    ("ActivateInviteOverlay", Stub::Nil),
    ("GetMaxCloudSaves", Stub::Num(0.0)),
    ("CopyLastAutoSaveToSteamCloud", Stub::Nil),
    ("GetCloudSaves", Stub::Table),
    ("ActivateGameOverlayToStore", Stub::Nil),
];

pub(crate) fn install<'l>(inner: &Rc<Inner>, lua: &'l Lua, g: &Table<'l>) -> Result<()> {
    let reg = Reg::new(inner);

    // The table the contexts share: EUI keeps its objects in `MapModData.UI_bc1`.
    g.raw_set("MapModData", lua.create_table())?;
    reg.scope("MapModData");

    reg.global_real(lua, g, "InStrategicView", |_, _: MultiValue| Ok(false))?;

    // SIMPLIFIED(SIMP-exe-20): no strategic view (U-D8), the game core is never busy, no unit is moving.
    for (name, ret) in GLOBAL_STUBS {
        reg.stub(lua, g, "", name, "SIMP-exe-20", *ret)?;
    }

    let steam = lua.create_table();
    for (name, ret) in STEAM {
        reg.stub(lua, &steam, "Steam", name, "SIMP-exe-12", *ret)?;
    }
    g.raw_set("Steam", steam)?;
    reg.scope("Steam");

    let mm = lua.create_table();
    let i = Rc::clone(inner);
    reg.real(
        lua,
        &mm,
        "Matchmaking",
        "IsHost",
        move |_, _: MultiValue| Ok(i.host.seat_info().is_host()),
    )?;
    let i = Rc::clone(inner);
    reg.real(
        lua,
        &mm,
        "Matchmaking",
        "GetLocalID",
        move |_, _: MultiValue| Ok(i.host.seat_info().local_id),
    )?;
    let i = Rc::clone(inner);
    reg.real(
        lua,
        &mm,
        "Matchmaking",
        "GetHostID",
        move |_, _: MultiValue| Ok(i.host.seat_info().host_id),
    )?;
    reg.stub(
        lua,
        &mm,
        "Matchmaking",
        "GetPlayerList",
        "SIMP-exe-12",
        Stub::Table,
    )?;
    reg.stub(
        lua,
        &mm,
        "Matchmaking",
        "KickPlayer",
        "SIMP-exe-12",
        Stub::Nil,
    )?;
    g.raw_set("Matchmaking", mm)?;
    reg.scope("Matchmaking");
    Ok(())
}
