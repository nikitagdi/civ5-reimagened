//! WO-C (milestone M3): what the corners of the HUD ask of the exe: the minimap handle, the chat, the MP
//! player list and the strategic-view option lists of the minimap panel.
//!
//! - `UI:RequestMinimapBroadcast()` answers `Events.MinimapTextureBroadcastEvent(handle, width, height)`; the handle
//!   is the renderer's minimap texture ([`MINIMAP_HANDLE`] = `draw_list::TextureId::MINIMAP`), the size is the host's
//!   ([`crate::ExeHost::minimap_size`]). `Events.MinimapClickedEvent(x, y)` carries the click in pixels of the control;
//!   the app turns it into a camera move.
//! - `Network.SendChat(text, team, player)` goes to the host ([`crate::ExeHost::send_chat`]); a received line comes
//!   back through [`crate::ExeApi::chat_received`] as `Events.GameMessageChat`.
//!
//! SIMPLIFIED(SIMP-exe-130): no strategic view (U-D8): its option lists (`GetStrategicViewOverlays`,
//! `GetStrategicViewIconSettings`) hold the names of the exe's lists and the setters do nothing.
//! SIMPLIFIED(SIMP-exe-131): `Matchmaking.KickPlayer` asks the host to kick; the list of players the original UI
//! reads is `Players[]` (the lobby slots of the exe are not modelled).
//! `UI.SetYieldVisibleMode` and `UI.SetResourceVisibleMode` drive the yield and resource icons of the map
//! (`yields.rs`, SIMP-exe-133); the grid toggle is the renderer's.
//! SIMPLIFIED(SIMP-exe-134): `UI.SetOfferTradeRepeatCount` is a counted stub (the other deal functions of `TradeLogic` are real since K12).
//! SIMPLIFIED(SIMP-exe-132): the chat copy that the Lekmod DiploCorner sends as `Network.SendEnhanceReligion` with the
//! religion -1 is dropped (the chat itself travels as `Network.SendChat`).

use std::rc::Rc;

use lua_sys::{Lua, LuaError, LuaValue, MultiValue, Result, Table};

use crate::Inner;
use crate::core::{Arg, Core};
use crate::registry::{Reg, Status, Stub};

/// The contexts of WO-C (M3), by bare name: K4 info corner lists, K5 diplomacy corner and chat, K6 minimap, K7 turn
/// flow and the MP panels. `m2_contexts` adds them to the list of the milestone.
pub const CONTEXTS: &[&str] = &[
    // K4
    "InfoCorner",
    "UnitList",
    "CityList",
    "ResourceList",
    "GPList",
    "TechPanel",
    // K5
    "DiploCorner",
    "DiploList",
    "SimpleDiploTrade",
    // K6
    "MiniMapPanel",
    // K7
    "NewTurn",
    "TurnProcessing",
    "MPTurnPanel",
    "MPList",
    "ConfirmKick",
];

/// `TextureId::MINIMAP`'s number as the Lua passes it to `SetTextureHandle`.
pub const MINIMAP_HANDLE: i32 = 1;

/// The names of the strategic view's overlay list (a 1-based table of text keys).
const SV_OVERLAYS: &[&str] = &[
    "TXT_KEY_SV_OVERLAY_NONE",
    "TXT_KEY_SV_OVERLAY_RESOURCES",
    "TXT_KEY_SV_OVERLAY_CULTURE",
    "TXT_KEY_SV_OVERLAY_RELIGION",
    "TXT_KEY_SV_OVERLAY_TRADE_ROUTES",
    "TXT_KEY_SV_OVERLAY_PASSABLE_TERRAIN",
    "TXT_KEY_SV_OVERLAY_UNIT_COMBAT_TYPE",
    "TXT_KEY_SV_OVERLAY_UNIT_DOMAIN",
    "TXT_KEY_SV_OVERLAY_UNIT_TYPE",
    "TXT_KEY_SV_OVERLAY_UNIT_CULTURE",
];

/// The icon settings of the strategic view.
const SV_ICONS: &[&str] = &[
    "TXT_KEY_SV_ICONS_ALL",
    "TXT_KEY_SV_ICONS_RESOURCES",
    "TXT_KEY_SV_ICONS_IMPROVEMENTS",
    "TXT_KEY_SV_ICONS_UNITS",
    "TXT_KEY_SV_ICONS_NONE",
];

/// `SlotStatus` and `SlotClaim` (`cvLuaUIEnumsLibrary`; the numbers are `SlotStatus` / `SlotClaim` of `CvPreGame`).
const SLOT_STATUS: &[(&str, i32)] = &[
    ("SS_CLOSED", 0),
    ("SS_OPEN", 1),
    ("SS_COMPUTER", 2),
    ("SS_OBSERVER", 3),
    ("SS_TAKEN", 4),
];
const SLOT_CLAIM: &[(&str, i32)] = &[
    ("SLOTCLAIM_UNASSIGNED", 0),
    ("SLOTCLAIM_RESERVED", 1),
    ("SLOTCLAIM_ASSIGNED", 2),
];

/// Marks a real function as the implementation of a SIMP row (it is a simplification, not a stub: no call counter).
fn simp_mark(inner: &Rc<Inner>, path: &str, id: &'static str) {
    inner.registry.borrow_mut().add(path, Status::Simp(id));
}

fn list<'l>(lua: &'l Lua, names: &[&str]) -> Result<Table<'l>> {
    let t = lua.create_table();
    for (i, n) in names.iter().enumerate() {
        t.raw_set(i as i64 + 1, *n)?;
    }
    Ok(t)
}

fn enum_table<'l>(lua: &'l Lua, values: &[(&str, i32)]) -> Result<Table<'l>> {
    let t = lua.create_table();
    for (k, v) in values {
        t.raw_set(*k, *v)?;
    }
    Ok(t)
}

pub(crate) fn install<'l>(
    core: &Rc<Core>,
    inner: &Rc<Inner>,
    lua: &'l Lua,
    g: &Table<'l>,
) -> Result<()> {
    let reg = Reg::new(inner);

    // The enums of the player slots, unless the game replica registered its own (it overwrites these later).
    for (name, values) in [("SlotStatus", SLOT_STATUS), ("SlotClaim", SLOT_CLAIM)] {
        if matches!(g.raw_get::<_, LuaValue>(name)?, LuaValue::Nil) {
            g.raw_set(name, enum_table(lua, values)?)?;
            reg.scope(name);
        }
    }

    // The strategic view's option lists.
    reg.real(
        lua,
        g,
        "",
        "GetStrategicViewOverlays",
        |lua, _: MultiValue| list(lua, SV_OVERLAYS),
    )?;
    simp_mark(inner, "GetStrategicViewOverlays", "SIMP-exe-130");
    reg.real(
        lua,
        g,
        "",
        "GetStrategicViewIconSettings",
        |lua, _: MultiValue| list(lua, SV_ICONS),
    )?;
    simp_mark(inner, "GetStrategicViewIconSettings", "SIMP-exe-130");
    reg.stub(
        lua,
        g,
        "",
        "SetStrategicViewOverlay",
        "SIMP-exe-130",
        Stub::Nil,
    )?;
    reg.stub(
        lua,
        g,
        "",
        "SetStrategicViewIconSetting",
        "SIMP-exe-130",
        Stub::Nil,
    )?;

    // The minimap handle.
    let ui: Table = g.raw_get("UI")?;
    // The map options of the minimap panel: the grid is the renderer's (the host), yields and resources are remembered only.
    let w = Rc::downgrade(core);
    reg.real(
        lua,
        &ui,
        "UI",
        "SetGridVisibleMode",
        move |_, on: Option<bool>| {
            if let Some(c) = w.upgrade() {
                let on = on.unwrap_or(false);
                c.ui.borrow_mut().grid_on = on;
                c.host.set_grid_visible(on);
                c.fire_events(
                    if on {
                        "SerialEventHexGridOn"
                    } else {
                        "SerialEventHexGridOff"
                    },
                    vec![],
                );
            }
            Ok(())
        },
    )?;
    // The yield and resource icons of the map (yields.rs): the boxes ask the exe for a display mode, which it
    // announces as `Events.RequestYieldDisplay` (its own `YieldIconManager` and the Lua managers listen).
    for (name, on, off) in [
        (
            "SetYieldVisibleMode",
            crate::yields::USER_ALL_ON,
            crate::yields::USER_ALL_OFF,
        ),
        (
            "SetResourceVisibleMode",
            crate::yields::USER_ALL_RESOURCE_ON,
            crate::yields::USER_ALL_RESOURCE_OFF,
        ),
    ] {
        let w = Rc::downgrade(core);
        reg.real(lua, &ui, "UI", name, move |_, v: Option<bool>| {
            if let Some(c) = w.upgrade() {
                let ty = if v.unwrap_or(false) { on } else { off };
                c.fire(crate::core::Fire {
                    hook: Some(crate::core::Hook::RequestYieldDisplay(ty, [0; 3])),
                    ..crate::core::Fire::events("RequestYieldDisplay", vec![Arg::from(ty)])
                });
            }
            Ok(())
        })?;
        simp_mark(inner, &format!("UI.{name}"), "SIMP-exe-133");
    }
    let w = Rc::downgrade(core);
    reg.real(
        lua,
        &ui,
        "UI",
        "RefreshYieldVisibleMode",
        move |_, _: MultiValue| {
            if let Some(c) = w.upgrade() {
                for f in c.yield_fires(true) {
                    c.enqueue(f);
                }
            }
            Ok(())
        },
    )?;
    simp_mark(inner, "UI.RefreshYieldVisibleMode", "SIMP-exe-133");
    reg.stub(lua, g, "", "GetOverlayLegend", "SIMP-exe-130", Stub::Nil)?;
    // The deal functions of the TradeLogic that SimpleDiploTrade includes: the trade window is K12 (M4).
    // K12 (WO-D) made the others real in `ui_api` (SIMP-exe-300).
    reg.stub(
        lua,
        &ui,
        "UI",
        "SetOfferTradeRepeatCount",
        "SIMP-exe-134",
        Stub::Nil,
    )?;
    let w = Rc::downgrade(core);
    reg.real(
        lua,
        &ui,
        "UI",
        "RequestMinimapBroadcast",
        move |_, _: MultiValue| {
            if let Some(c) = w.upgrade() {
                let (width, height) = c.host.minimap_size();
                c.fire_events(
                    "MinimapTextureBroadcastEvent",
                    vec![
                        Arg::from(MINIMAP_HANDLE),
                        Arg::from(width as i32),
                        Arg::from(height as i32),
                    ],
                );
            }
            Ok(())
        },
    )?;

    // The chat. `Network.SendChat(text, team, player)`: the server has `All` and one
    // player; a line to a team goes to each human of it (SIMP-exe-912).
    let net: Table = g.raw_get("Network")?;
    let w = Rc::downgrade(core);
    reg.real(
        lua,
        &net,
        "Network",
        "SendChat",
        move |lua, args: MultiValue| {
            let c = w
                .upgrade()
                .ok_or_else(|| LuaError::msg("the exe API is gone"))?;
            let v = args.into_vec();
            let text = match v.first() {
                Some(LuaValue::String(s)) => s.to_string_lossy().into_owned(),
                _ => return Ok(()),
            };
            let player = match v.get(2) {
                Some(LuaValue::Number(n)) if *n >= 0.0 => Some(*n as i32),
                _ => None,
            };
            let team = match v.get(1) {
                Some(LuaValue::Number(n)) if *n >= 0.0 => Some(*n as i32),
                _ => None,
            };
            if player.is_none()
                && let Some(team) = team
            {
                // SIMPLIFIED(SIMP-exe-912): the server has "all" and "one player" as addressees, so a team line goes as one
                // direct line to each human of the team (the sender's own seat when it is alone), not to everybody; the
                // team's members read it as a direct line from the sender.
                for to in c.team_chat_targets(lua, team) {
                    c.host.send_chat(Some(to), &text);
                }
                return Ok(());
            }
            c.host.send_chat(player, &text);
            Ok(())
        },
    )?;
    // The chat copy of the Lekmod DiploCorner (SIMP-exe-132).
    lua.load(
        r#"local f = Network.SendEnhanceReligion
           if f then
             Network.SendEnhanceReligion = function(p, religion, ...)
               if religion == -1 then return end
               return f(p, religion, ...)
             end
           end"#,
    )
    .set_name("=corners")
    .set_environment(g.clone())
    .exec()?;

    // `Matchmaking.KickPlayer(playerId)`.
    let mm: Table = g.raw_get("Matchmaking")?;
    let w = Rc::downgrade(core);
    reg.real(
        lua,
        &mm,
        "Matchmaking",
        "KickPlayer",
        move |_, id: Option<i32>| {
            if let (Some(c), Some(id)) = (w.upgrade(), id) {
                c.host.kick_player(id);
            }
            Ok(())
        },
    )?;
    simp_mark(inner, "Matchmaking.KickPlayer", "SIMP-exe-131");
    Ok(())
}

/// `PreGame` strings the replica's `PreGame` lacks: the custom names of a civilization (the scripts fall back to
/// `GameInfo.Civilizations` on the empty string). Call after the replica registered `PreGame` and the client part is on
/// top of it (`ExeApi::bind_game`). SIMPLIFIED(SIMP-exe-137): they answer "" (no custom civilization names in MP).
pub(crate) fn install_pregame_names(inner: &Rc<Inner>, lua: &Lua) -> Result<()> {
    let g = lua.globals();
    let LuaValue::Table(pre) = g.raw_get::<_, LuaValue>("PreGame")? else {
        return Ok(());
    };
    let reg = Reg::new(inner);
    for name in [
        "GetCivilizationShortDescription",
        "GetCivilizationDescription",
        "GetCivilizationAdjective",
        // K12: GameplayUtilities.GetPlayerName (the leader screen) reads the custom leader name too.
        "GetLeaderName",
    ] {
        if matches!(pre.raw_get::<_, LuaValue>(name)?, LuaValue::Nil) {
            reg.real(lua, &pre, "PreGame", name, |_, _: MultiValue| {
                Ok(String::new())
            })?;
            simp_mark(inner, &format!("PreGame.{name}"), "SIMP-exe-137");
        }
    }
    Ok(())
}

impl Core {
    /// A chat line arrived: `Events.GameMessageChat(from, to, text, targetType, skipSound, turn, timestamp)`.
    /// `to` is a player id or -1; the target type is `ChatTargetTypes` (ALL 0, TEAM 1, PLAYER 2).
    pub fn chat_received(&self, from: i32, to: Option<i32>, text: &str, turn: i32, stamp: i32) {
        let (target, to) = match to {
            Some(p) => (2, p),
            None => (0, -1),
        };
        self.fire_events(
            "GameMessageChat",
            vec![
                Arg::from(from),
                Arg::from(to),
                Arg::Str(text.to_owned()),
                Arg::from(target),
                Arg::Bool(false),
                Arg::from(turn),
                Arg::from(stamp),
            ],
        );
    }
}

impl Core {
    /// The human seats of team `team` (the sender's own seat when it is the only one), for a team chat line.
    fn team_chat_targets(&self, lua: &Lua, team: i32) -> Vec<i32> {
        let me = self.host.seat_info().local_id;
        let found: Vec<i32> = lua
            .load(&format!(
                "local t, out = {team}, {{}}                  for i = 0, GameDefines.MAX_MAJOR_CIVS - 1 do                    local p = Players[i]                    if p and p:IsHuman() and p:GetTeam() == t and not (p.IsObserver and p:IsObserver()) then out[#out + 1] = i end                  end                  return out"
            ))
            .set_name("=exe_team_chat")
            .eval::<Table>()
            .and_then(|t| t.sequence_values::<i32>())
            .unwrap_or_default();
        if found.is_empty() { vec![me] } else { found }
    }
}

impl crate::ExeApi {
    /// A unit moved (`CvGame::unitIsMoving`): the clock of the auto end of turn (tests; the replica's events call it).
    #[doc(hidden)]
    pub fn note_unit_moved(&self) {
        self.core.note_unit_moved();
    }

    /// A chat line the server delivered (see [`Core::chat_received`]).
    pub fn chat_received(&self, from: i32, to: Option<i32>, text: &str, turn: i32, stamp: i32) {
        self.core.chat_received(from, to, text, turn, stamp);
    }

    /// The minimap texture changed (its size or generation): the panel gets a new `MinimapTextureBroadcastEvent`.
    pub fn broadcast_minimap(&self) {
        let (w, h) = self.core.host.minimap_size();
        self.core.fire_events(
            "MinimapTextureBroadcastEvent",
            vec![
                Arg::from(MINIMAP_HANDLE),
                Arg::from(w as i32),
                Arg::from(h as i32),
            ],
        );
    }
}

impl crate::ExeApi {
    /// The UI sent `Network.SendTurnComplete` for this turn and has not taken it back (the MP turn panel, tests).
    pub fn turn_complete_sent(&self) -> bool {
        self.core.ui.borrow().sent_turn_complete
    }

    /// The server refused (or lost) the end of turn the UI sent: the turn is not ended, so
    /// `Network.HasSentNetTurnComplete` is false again and the end-turn button is drawn anew (it shows "End
    /// turn" or the blocker, not "Waiting for players"; live game 931, 2026-10-30). `true` when the flag was set.
    ///
    /// Not once the replica has every human's end of turn, the seat's own included: the server took it, the turn is
    /// being processed, and the refusal is of a duplicate (an order stamped with the next turn while the AI moved).
    pub fn turn_complete_refused(&self) -> bool {
        if self.core.turn_all_confirmed() {
            tracing::info!(target: "exe.net", "an end of turn refused while the turn is processed: kept");
            return false;
        }
        let was = std::mem::replace(&mut self.core.ui.borrow_mut().sent_turn_complete, false);
        // The router has nothing to take back before the next order either.
        if let Some(g) = self.core.game()
            && let Ok(mut g) = g.try_borrow_mut()
        {
            g.set_turn_sent(false);
        }
        if was {
            self.core.fire_events("SerialEventEndTurnDirty", vec![]);
        }
        was
    }
}
