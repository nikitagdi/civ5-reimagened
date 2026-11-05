//! The client part of `PreGame`. The reads of `CvPreGame` come from the game
//! replica (`gc_lua_attach_ui` registers `PreGame`); what the client alone knows is put on top of the
//! same table: `GetNickName`, `IsMultiplayerGame` (true), `GameStarted` (true), `IsHotSeatGame` (false),
//! `IsInternetGame` (true) and `SetLeaderKey` / `GetLeaderKey` (the EUI keeps a hotkey there).
//!
//! `install` finds the `PreGame` table that exists and adds to it, so it can run before and after the
//! replica attached (`ExeApi::reinstall_pregame`).
//!
//! SIMPLIFIED(SIMP-exe-18): the leader keys live in memory only.

use std::rc::Rc;

use lua_sys::{Lua, LuaString, LuaValue, MultiValue, Result, Table};

use crate::Inner;
use crate::registry::Reg;

pub(crate) fn install<'l>(inner: &Rc<Inner>, lua: &'l Lua, g: &Table<'l>) -> Result<()> {
    let reg = Reg::new(inner);
    let t: Table = match g.raw_get::<_, LuaValue>("PreGame")? {
        LuaValue::Table(t) => t,
        _ => {
            let t = lua.create_table();
            g.raw_set("PreGame", t.clone())?;
            t
        }
    };
    reg.scope("PreGame");

    let i = Rc::clone(inner);
    reg.real(
        lua,
        &t,
        "PreGame",
        "GetNickName",
        move |_, _: MultiValue| Ok(i.host.seat_info().nick),
    )?;
    reg.real(
        lua,
        &t,
        "PreGame",
        "IsMultiplayerGame",
        |_, _: MultiValue| Ok(true),
    )?;
    reg.real(lua, &t, "PreGame", "GameStarted", |_, _: MultiValue| {
        Ok(true)
    })?;
    reg.real(lua, &t, "PreGame", "IsHotSeatGame", |_, _: MultiValue| {
        Ok(false)
    })?;
    reg.real(lua, &t, "PreGame", "IsInternetGame", |_, _: MultiValue| {
        Ok(true)
    })?;

    let i = Rc::clone(inner);
    reg.real(
        lua,
        &t,
        "PreGame",
        "SetLeaderKey",
        move |_, args: MultiValue| {
            let player = match args.get(0) {
                Some(LuaValue::Number(n)) => *n as i32,
                _ => return Ok(()),
            };
            match args.get(1) {
                Some(LuaValue::String(s)) => {
                    i.leader_keys
                        .borrow_mut()
                        .insert(player, s.to_string_lossy().into_owned());
                }
                _ => {
                    i.leader_keys.borrow_mut().remove(&player);
                }
            }
            Ok(())
        },
    )?;
    let i = Rc::clone(inner);
    reg.real(
        lua,
        &t,
        "PreGame",
        "GetLeaderKey",
        move |_, args: MultiValue| {
            let player = match args.get(0) {
                Some(LuaValue::Number(n)) => *n as i32,
                _ => return Ok(LuaValue::Nil),
            };
            Ok(i.leader_keys
                .borrow()
                .get(&player)
                .map(|s| LuaValue::String(LuaString(s.clone().into_bytes())))
                .unwrap_or(LuaValue::Nil))
        },
    )?;

    // The replica's `PreGame` has no `GetCivilizationColor`; the colour set of a seat is the player's
    // own colour, else the default of its civilization (what the EUI falls back to as well).
    if matches!(
        t.raw_get::<_, LuaValue>("GetCivilizationColor")?,
        LuaValue::Nil
    ) {
        let f: LuaValue = lua
            .load(CIV_COLOR_GLUE)
            .set_name("=exe_pregame_color")
            .set_environment(g.clone())
            .eval()?;
        reg.value(&t, "PreGame", "GetCivilizationColor", f)?;
    }
    Ok(())
}

const CIV_COLOR_GLUE: &str = r#"
return function(playerID)
    local p = Players and Players[playerID]
    if p and p.GetPlayerColor then
        local ok, c = pcall(p.GetPlayerColor, p)
        if ok and c and c >= 0 then return c end
    end
    local civType = PreGame.GetCivilization and PreGame.GetCivilization(playerID)
    local civ = civType and GameInfo.Civilizations[civType]
    local set = civ and GameInfo.PlayerColors[civ.DefaultPlayerColor or -1]
    return set and set.ID or -1
end
"#;
