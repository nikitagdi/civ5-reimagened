//! Milestone M4, work order G: K19 (end of the game) and K20 (game menu, options, save, exit).
//!
//! The names of the contexts are bare Lua file names; `m2_contexts()` appends them to the earlier lists.
//!
//! What the windows ask of the exe and this module gives:
//! - the options menu (`OptionsMenu`, EUI/Lekmod): the whole `OptionsManager` of the menu ([`options_menu`]): every
//!   `Get<X>_Cached` / `Set<X>_Cached`, the commit and sync calls, the resets. The values are kept in the host's store
//!   as `civ5.options.<Key>` (the page's `localStorage`: they stick between sessions of this client); the volume
//!   knobs (`GetVolumeKnobValue`, `SetVolumeKnobValue`, `SaveAudioOptions`) drive the UI sound gain through
//!   [`crate::ExeHost::set_volume`];
//! - the game menu: `UI.SaveGame` (the host asks the server for a save), `Events.ExitToMainMenu` and friends (the
//!   player leaves for the lobby), the Save button that only the host sees, `UI.GetMapPreview`;
//! - the end of the game: `Events.EndGameShow(type, team)`, raised by [`crate::core::Core::check_game_over`] when the
//!   replica loads the final state of a game that is over (the exe raises it from `CvGame::showEndGameSequence`; the
//!   replica runs no turn, so no core event comes), `EndGameTypes`.
//!
//! SIMPLIFIED(SIMP-exe-390): `LoadMenu`, `SaveMapMenu` and `TouchControlsMenu` (single player, touch) stay empty and
//!   hidden; "Exit to Windows" leaves for the lobby like "Exit to main menu".
//! SIMPLIFIED(SIMP-exe-391): the graphics, resolution, language, e-mail notification options of the options menu are
//!   stored and read back but drive nothing (a browser tab); the menu lists one resolution.
//! SIMPLIFIED(SIMP-exe-392): each volume knob is the gain of its own bus (music, effects, speech, ambience) and applies
//!   at once (the exe ramps it); the knobs start at the page's `?volume=` (SIMP-exe-190).
//! SIMPLIFIED(SIMP-exe-393): `UI.SaveGame` asks the server for a save of the game (the host only, the server names
//!   it); the list of saves of the save menu is empty, quick save, delete and cloud saves do nothing.
//! SIMPLIFIED(SIMP-exe-394): the Save button of the game menu is hidden for a guest by an answer of
//!   `Modding.AnyActivatedModsContainPropertyValue("DisableSaveGameOption")` (the Lekmod menu shows it to everybody).
//! SIMPLIFIED(SIMP-exe-942): `UI.GetReplayInfo` and the front-end replay file functions do not exist (the replay tab reads the running game).
//! SIMPLIFIED(SIMP-exe-395): `EndGameTypes` has our numbers; extended play (`Network.SendExtendedGame`) does nothing:
//!   the server ends the game, the window only closes.

use std::rc::Rc;

use lua_sys::{Lua, LuaString, LuaValue, MultiValue, Result, Table};
use platform::KeyValueStore;

use crate::Inner;
use crate::core::Core;
use crate::options::{KEY_PREFIX, OPTIONS};
use crate::registry::{Reg, Status, Stub};

/// K20: the game menu (Esc), its save and options windows. `LoadMenu`, `SaveMapMenu` and `TouchControlsMenu` are
/// single-player / touch windows and stay empty (SIMP-exe-390).
pub const K20: &[&str] = &["GameMenu", "SaveMenu", "OptionsMenu"];

/// K19: the end-of-game window and its tabs (the replay tab is `ReplayViewer`, the demographics tab `Demographics`).
pub const K19: &[&str] = &["EndGameMenu", "Ranking", "ReplayViewer", "Demographics"];

/// The volume knobs of the options menu, in the order of their ids (`GetVolumeKnobIDFromName("USER_VOLUME_<name>")`).
pub const KNOBS: [&str; 4] = ["MUSIC", "SFX", "SPEECH", "AMBIENCE"];

/// `EndGameTypes` (SIMP-exe-395: the numbers are ours; the scripts use the names).
pub const END_GAME_TYPES: &[(&str, i32)] = &[
    ("Tutorial1", 0),
    ("Tutorial2", 1),
    ("Tutorial3", 2),
    ("Tutorial4", 3),
    ("Tutorial5", 4),
    ("Technology", 5),
    ("Domination", 6),
    ("Culture", 7),
    ("Diplomatic", 8),
    ("Time", 9),
    ("Loss", 10),
];

/// `GameStateTypes` of the exe (the options menu asks whether it is up in the main menu).
const GAME_STATES: &[(&str, i32)] = &[
    ("CIV5_GS_NONE", 0),
    ("CIV5_GS_MAIN_MENU", 1),
    ("CIV5_GS_LOADING", 2),
    ("CIV5_GS_GAME", 3),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum K {
    Bool,
    Num,
    Str,
}

/// An option of the menu: the getter of the committed value, the key in the store and the default (store text).
struct G {
    getter: &'static str,
    key: &'static str,
    kind: K,
    default: &'static str,
}

const fn gb(getter: &'static str, key: &'static str, default: &'static str) -> G {
    G {
        getter,
        key,
        kind: K::Bool,
        default,
    }
}

const fn gn(getter: &'static str, key: &'static str, default: &'static str) -> G {
    G {
        getter,
        key,
        kind: K::Num,
        default,
    }
}

const fn gs(getter: &'static str, key: &'static str) -> G {
    G {
        getter,
        key,
        kind: K::Str,
        default: "",
    }
}

/// The options `options::OPTIONS` has no getter for; the defaults are those of the `UserSettings.ini` the game writes.
const EXTRA: &[G] = &[
    gb("IsAutoWorkersDontReplace", "AutoWorkersDontReplace", "0"),
    gb(
        "IsAutoWorkersDontRemoveFeatures",
        "AutoWorkersDontRemoveFeatures",
        "0",
    ),
    gb(
        "GetSinglePlayerAutoEndTurnEnabled",
        "SinglePlayerAutoEndTurn",
        "0",
    ),
    gb(
        "GetMultiplayerAutoEndTurnEnabled",
        "MultiplayerAutoEndTurn",
        "0",
    ),
    gb(
        "GetQuickSelectionAdvanceEnabled",
        "QuickSelectionAdvance",
        "1",
    ),
    gb(
        "GetSinglePlayerQuickCombatEnabled",
        "SinglePlayerQuickCombat",
        "0",
    ),
    gb(
        "GetSinglePlayerQuickMovementEnabled",
        "SinglePlayerQuickMovement",
        "0",
    ),
    gb(
        "GetMultiplayerQuickCombatEnabled",
        "MultiplayerQuickCombat",
        "0",
    ),
    gb(
        "GetMultiplayerQuickMovementEnabled",
        "MultiplayerQuickMovement",
        "0",
    ),
    gb("GetStraightZoom", "StraightZoom", "0"),
    gb("GetAutoUIAssets", "AutoUIAssets", "1"),
    gb("GetEnableMapInertia", "EnableMapInertia", "1"),
    gb("GetSkipIntroVideo", "SkipIntroVideo", "0"),
    gb("GetVSync", "VSync", "1"),
    gb("GetHDStrategicView", "HDStrategicView", "1"),
    gb("GetGPUTextureDecode", "GPUTextureDecode", "0"),
    gb("GetMinimizeGrayTiles", "MinimizeGrayTiles", "0"),
    gb("GetFadeShadows", "FadeShadows", "1"),
    gb("GetTurnNotifySteamInvite", "TurnNotifySteamInvite", "0"),
    gb("GetTurnNotifyEmail", "TurnNotifyEmail", "0"),
    gb("GetTurnNotifySmtpTLS", "TurnNotifySmtpTLS", "0"),
    gn("GetTurnsBetweenAutosave", "TurnsBetweenAutosave", "1"),
    gn("GetNumAutosavesKept", "NumAutosavesKept", "10"),
    gn("GetBindMouseMode", "BindMouseMode", "1"),
    gn("GetTutorialLevel", "TutorialLevel", "0"),
    gn("GetDragSpeed", "DragSpeed", "1"),
    gn("GetPinchSpeed", "PinchSpeed", "1"),
    gn("GetAASamples", "AASamples", "0"),
    gn("GetLeaderQuality", "LeaderQuality", "2"),
    gn("GetOverlayLevel", "OverlayLevel", "2"),
    gn("GetShadowLevel", "ShadowLevel", "2"),
    gn("GetFOWLevel", "FOWLevel", "2"),
    gn("GetTerrainDetailLevel", "TerrainDetailLevel", "2"),
    gn("GetTerrainTessLevel", "TerrainTessLevel", "1"),
    gn("GetTerrainShadowQuality", "TerrainShadowQuality", "2"),
    gn("GetWaterQuality", "WaterQuality", "1"),
    gn("GetTextureQuality", "TextureQuality", "1"),
    gn("GetResolution", "Resolution", "0"),
    gn("GetTurnNotifySmtpPort", "TurnNotifySmtpPort", "25"),
    gs("GetTurnNotifyEmailAddress", "TurnNotifyEmailAddress"),
    gs(
        "GetTurnNotifySmtpEmailAddress",
        "TurnNotifySmtpEmailAddress",
    ),
    gs("GetTurnNotifySmtpHost", "TurnNotifySmtpHost"),
    gs("GetTurnNotifySmtpUsername", "TurnNotifySmtpUsername"),
    gs("GetTurnNotifySmtpPassword", "TurnNotifySmtpPassword"),
    gs("GetLANNickName", "LANNickName"),
];

/// `Set<Rest>` of `Get<Rest>` / `Is<Rest>`.
fn setter_of(getter: &str) -> String {
    let rest = getter
        .strip_prefix("Get")
        .or_else(|| getter.strip_prefix("Is"))
        .unwrap_or(getter);
    format!("Set{rest}")
}

fn store_key(key: &str) -> String {
    format!("{KEY_PREFIX}{key}")
}

/// The store text of a Lua value for an option of `kind`.
fn store_text(kind: K, v: Option<&LuaValue<'_>>) -> String {
    match (kind, v) {
        (K::Bool, Some(LuaValue::Boolean(b))) => u8::from(*b).to_string(),
        (K::Bool, Some(LuaValue::Number(n))) => u8::from(*n != 0.0).to_string(),
        (K::Bool, _) => "0".to_owned(),
        (K::Num, Some(LuaValue::Number(n))) => {
            if n.fract() == 0.0 {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        (K::Num, Some(LuaValue::String(s))) => s
            .to_string_lossy()
            .trim()
            .parse::<f64>()
            .map_or_else(|_| "0".to_owned(), |n| n.to_string()),
        (K::Num, Some(LuaValue::Boolean(b))) => u8::from(*b).to_string(),
        (K::Num, _) => "0".to_owned(),
        (K::Str, Some(LuaValue::String(s))) => s.to_string_lossy().into_owned(),
        (K::Str, Some(LuaValue::Number(n))) => n.to_string(),
        (K::Str, _) => String::new(),
    }
}

fn lua_value<'l>(kind: K, text: &str) -> LuaValue<'l> {
    match kind {
        K::Bool => LuaValue::Boolean(text.trim() == "1"),
        K::Num => LuaValue::Number(text.trim().parse::<f64>().unwrap_or(0.0)),
        K::Str => LuaValue::String(lua_sys::LuaString(text.as_bytes().to_vec())),
    }
}

/// The value of an option now: pending (set by the menu, not committed), else committed, else the default.
fn value_now(inner: &Inner, key: &str, default: &str) -> String {
    if let Some(v) = inner.options.pending_gen.borrow().get(key) {
        return v.clone();
    }
    if let Some(v) = inner.options.pending.borrow().get(key) {
        return v.to_string();
    }
    inner
        .host
        .kv()
        .get(&store_key(key))
        .unwrap_or_else(|| default.to_owned())
}

/// Every option the menu can change: (key, default text) of the table of `options` and of [`EXTRA`].
fn all_options() -> impl Iterator<Item = (&'static str, String)> {
    OPTIONS
        .iter()
        .map(|o| (o.key, o.default.to_string()))
        .chain(EXTRA.iter().map(|g| (g.key, g.default.to_owned())))
        .chain([
            ("WindowResX", "1920".to_owned()),
            ("WindowResY", "1080".to_owned()),
        ])
}

fn simp_mark(inner: &Rc<Inner>, path: &str, id: &'static str) {
    inner.registry.borrow_mut().add(path, Status::Simp(id));
}

/// The volume knobs (0..=1): the store, else the page's default.
fn volumes(inner: &Inner) -> [f32; 4] {
    if let Some(v) = *inner.options.volumes.borrow() {
        return v;
    }
    let v = stored_volume_knobs(inner.host.kv(), inner.host.volume_default());
    *inner.options.volumes.borrow_mut() = Some(v);
    v
}

/// The volume knobs the player saved (`SaveAudioOptions`), in the order of [`KNOBS`]; `default` for a knob never saved.
/// The app reads them before a game, so the main menu's music has its slider too.
pub fn stored_volume_knobs(kv: &dyn KeyValueStore, default: f32) -> [f32; 4] {
    let mut v = [default.clamp(0.0, 1.0); 4];
    for (i, n) in KNOBS.iter().enumerate() {
        if let Some(s) = kv.get(&store_key(&format!("Volume.{n}")))
            && let Ok(x) = s.trim().parse::<f32>()
        {
            v[i] = x.clamp(0.0, 1.0);
        }
    }
    v
}

pub(crate) fn install<'l>(
    core: &Rc<Core>,
    inner: &Rc<Inner>,
    lua: &'l Lua,
    g: &Table<'l>,
) -> Result<()> {
    let reg = Reg::new(inner);
    options_menu(core, &reg, inner, lua, g)?;
    volume_knobs(&reg, inner, lua, g)?;
    locale_and_enums(&reg, inner, lua, g)?;
    ui_and_game_menu(core, &reg, inner, lua, g)?;
    Ok(())
}

/// `OptionsManager` for the options menu: cached getters and setters, commit, sync, reset.
fn options_menu<'l>(
    core: &Rc<Core>,
    reg: &Reg<'_>,
    inner: &Rc<Inner>,
    lua: &'l Lua,
    g: &Table<'l>,
) -> Result<()> {
    let t: Table = g.raw_get("OptionsManager")?;
    let missing = |name: &str| matches!(t.raw_get::<_, LuaValue>(name), Ok(LuaValue::Nil));

    // Getters of the committed value (for the options `options.rs` has none for), the cached getters, both setters.
    let mut table: Vec<(String, &'static str, K, &'static str, bool)> = Vec::new();
    for o in OPTIONS {
        let kind = match o.kind {
            crate::options::Kind::Bool => K::Bool,
            crate::options::Kind::Int => K::Num,
        };
        // The committed getter exists already.
        table.push((o.getter.to_owned(), o.key, kind, "", true));
    }
    for e in EXTRA {
        table.push((e.getter.to_owned(), e.key, e.kind, e.default, false));
    }
    for (getter, key, kind, default, has_getter) in table {
        let default_text = if has_getter {
            OPTIONS
                .iter()
                .find(|o| o.key == key)
                .map_or_else(String::new, |o| o.default.to_string())
        } else {
            default.to_owned()
        };
        let cached = format!("{getter}_Cached");
        let setter = setter_of(&getter);
        let set_cached = format!("{setter}_Cached");
        if !has_getter && missing(&getter) {
            let (i, d) = (Rc::clone(inner), default_text.clone());
            reg.real(
                lua,
                &t,
                "OptionsManager",
                &getter,
                move |_, _: MultiValue| {
                    let v = i
                        .host
                        .kv()
                        .get(&store_key(key))
                        .unwrap_or_else(|| d.clone());
                    Ok(lua_value(kind, &v))
                },
            )?;
        }
        if missing(&cached) {
            let (i, d) = (Rc::clone(inner), default_text.clone());
            reg.real(
                lua,
                &t,
                "OptionsManager",
                &cached,
                move |_, _: MultiValue| Ok(lua_value(kind, &value_now(&i, key, &d))),
            )?;
        }
        if missing(&set_cached) {
            let i = Rc::clone(inner);
            reg.real(
                lua,
                &t,
                "OptionsManager",
                &set_cached,
                move |_, args: MultiValue| {
                    let text = store_text(kind, args.get(0));
                    i.options.pending.borrow_mut().remove(key);
                    i.options
                        .pending_gen
                        .borrow_mut()
                        .insert(key.to_owned(), text);
                    Ok(())
                },
            )?;
        }
        if missing(&setter) {
            // The uncached setter: set and commit at once.
            let (i, c) = (Rc::clone(inner), Rc::downgrade(core));
            reg.real(
                lua,
                &t,
                "OptionsManager",
                &setter,
                move |_, args: MultiValue| {
                    let text = store_text(kind, args.get(0));
                    i.host.kv().set(&store_key(key), &text);
                    if let Some(c) = c.upgrade() {
                        c.fire_events("GameOptionsChanged", vec![]);
                    }
                    Ok(())
                },
            )?;
        }
    }

    // The window resolution is a pair; the fullscreen resolution is an index (one mode, SIMP-exe-391).
    for (name, cached) in [
        ("GetWindowResolution", false),
        ("GetWindowResolution_Cached", true),
    ] {
        let i = Rc::clone(inner);
        reg.real(lua, &t, "OptionsManager", name, move |_, _: MultiValue| {
            let v = |k: &str| {
                if cached {
                    value_now(&i, k, "")
                } else {
                    i.host.kv().get(&store_key(k)).unwrap_or_default()
                }
            };
            let num = |s: String, d: f64| s.trim().parse::<f64>().unwrap_or(d);
            Ok((num(v("WindowResX"), 1920.0), num(v("WindowResY"), 1080.0)))
        })?;
    }
    let i = Rc::clone(inner);
    reg.real(
        lua,
        &t,
        "OptionsManager",
        "SetWindowResolution_Cached",
        move |_, args: MultiValue| {
            let mut p = i.options.pending_gen.borrow_mut();
            p.insert("WindowResX".into(), store_text(K::Num, args.get(0)));
            p.insert("WindowResY".into(), store_text(K::Num, args.get(1)));
            Ok(())
        },
    )?;
    reg.real(
        lua,
        &t,
        "OptionsManager",
        "GetMaxResolution",
        |_, _: MultiValue| Ok((2560, 1600)),
    )?;
    reg.real(
        lua,
        &t,
        "OptionsManager",
        "IsAALevelSupported",
        |_, _: MultiValue| Ok(true),
    )?;
    reg.real(
        lua,
        &t,
        "OptionsManager",
        "IsGPUTextureDecodeSupported",
        |_, _: MultiValue| Ok(false),
    )?;
    for name in [
        "GetMaxResolution",
        "IsAALevelSupported",
        "IsGPUTextureDecodeSupported",
        "GetWindowResolution",
        "GetWindowResolution_Cached",
        "SetWindowResolution_Cached",
    ] {
        simp_mark(inner, &format!("OptionsManager.{name}"), "SIMP-exe-391");
    }

    // Commit: everything pending is stored; the menu hears `GameOptionsChanged`.
    for name in [
        "CommitGameOptions",
        "CommitGraphicsOptions",
        "CommitResolutionOptions",
    ] {
        let (i, c) = (Rc::clone(inner), Rc::downgrade(core));
        reg.real(lua, &t, "OptionsManager", name, move |_, _: MultiValue| {
            let ints = std::mem::take(&mut *i.options.pending.borrow_mut());
            let gens = std::mem::take(&mut *i.options.pending_gen.borrow_mut());
            let changed = !ints.is_empty() || !gens.is_empty();
            for (key, v) in ints {
                i.host.kv().set(&store_key(key), &v.to_string());
            }
            for (key, v) in gens {
                i.host.kv().set(&store_key(&key), &v);
            }
            if changed && let Some(c) = c.upgrade() {
                c.fire_events("GameOptionsChanged", vec![]);
            }
            Ok(())
        })?;
        if name != "CommitGameOptions" {
            simp_mark(inner, &format!("OptionsManager.{name}"), "SIMP-exe-13");
        }
    }
    // Sync: the menu was cancelled or opens again; what was pending is dropped.
    for name in [
        "SyncGameOptionsCache",
        "SyncGraphicsOptionsCache",
        "SyncResolutionOptionsCache",
    ] {
        let i = Rc::clone(inner);
        reg.real(lua, &t, "OptionsManager", name, move |_, _: MultiValue| {
            i.options.pending.borrow_mut().clear();
            i.options.pending_gen.borrow_mut().clear();
            Ok(())
        })?;
        simp_mark(inner, &format!("OptionsManager.{name}"), "SIMP-exe-13");
    }
    // Reset to the defaults (pending until the menu's OK).
    for name in ["ResetDefaultGameOptions", "ResetDefaultGraphicsOptions"] {
        let i = Rc::clone(inner);
        reg.real(lua, &t, "OptionsManager", name, move |_, _: MultiValue| {
            let mut p = i.options.pending_gen.borrow_mut();
            for (key, default) in all_options() {
                p.insert(key.to_owned(), default);
            }
            i.options.pending.borrow_mut().clear();
            Ok(())
        })?;
    }
    reg.stub(
        lua,
        &t,
        "OptionsManager",
        "ResetTutorial",
        "SIMP-exe-391",
        Stub::Nil,
    )?;
    Ok(())
}

/// `GetVolumeKnobIDFromName`, `GetVolumeKnobValue`, `SetVolumeKnobValue`, `SaveAudioOptions` (globals of the exe).
fn volume_knobs<'l>(reg: &Reg<'_>, inner: &Rc<Inner>, lua: &'l Lua, g: &Table<'l>) -> Result<()> {
    reg.global_real(lua, g, "GetVolumeKnobIDFromName", |_, args: MultiValue| {
        let name = match args.get(0) {
            Some(LuaValue::String(s)) => s.to_string_lossy().into_owned(),
            _ => String::new(),
        };
        let short = name.strip_prefix("USER_VOLUME_").unwrap_or(&name);
        Ok(KNOBS
            .iter()
            .position(|k| *k == short)
            .map_or(-1, |p| p as i32))
    })?;
    let i = Rc::clone(inner);
    reg.global_real(lua, g, "GetVolumeKnobValue", move |_, args: MultiValue| {
        let id = match args.get(0) {
            Some(LuaValue::Number(n)) => *n as i32,
            _ => -1,
        };
        Ok(usize::try_from(id)
            .ok()
            .and_then(|id| volumes(&i).get(id).copied())
            .map_or(0.0, f64::from))
    })?;
    let i = Rc::clone(inner);
    reg.global_real(lua, g, "SetVolumeKnobValue", move |_, args: MultiValue| {
        let (Some(LuaValue::Number(id)), Some(LuaValue::Number(v))) = (args.get(0), args.get(1))
        else {
            return Ok(());
        };
        let Some(name) = usize::try_from(*id as i32)
            .ok()
            .and_then(|id| KNOBS.get(id))
        else {
            return Ok(());
        };
        let v = (*v as f32).clamp(0.0, 1.0);
        let mut all = volumes(&i);
        all[*id as usize] = v;
        *i.options.volumes.borrow_mut() = Some(all);
        i.host.set_volume(name, v);
        Ok(())
    })?;
    let i = Rc::clone(inner);
    reg.global_real(lua, g, "SaveAudioOptions", move |_, _: MultiValue| {
        let all = volumes(&i);
        for (n, v) in KNOBS.iter().zip(all) {
            i.host
                .kv()
                .set(&store_key(&format!("Volume.{n}")), &v.to_string());
        }
        Ok(())
    })?;
    for name in [
        "GetVolumeKnobIDFromName",
        "GetVolumeKnobValue",
        "SetVolumeKnobValue",
        "SaveAudioOptions",
    ] {
        simp_mark(inner, name, "SIMP-exe-392");
    }
    // The knobs the player set in an earlier session take effect at once.
    let all = volumes(inner);
    for (n, v) in KNOBS.iter().zip(all) {
        inner.host.set_volume(n, v);
    }
    Ok(())
}

/// `Locale.*` of the options menu and the enum tables of the end game and of the options menu.
fn locale_and_enums<'l>(
    reg: &Reg<'_>,
    inner: &Rc<Inner>,
    lua: &'l Lua,
    g: &Table<'l>,
) -> Result<()> {
    let loc: Table = g.raw_get("Locale")?;
    // `Locale.ToPercent(0.5)` -> 50.
    reg.real(lua, &loc, "Locale", "ToPercent", |_, args: MultiValue| {
        Ok(match args.get(0) {
            Some(LuaValue::Number(n)) => (n * 100.0).round(),
            _ => 0.0,
        })
    })?;
    let language = |lua: &'l Lua| -> Result<Table<'l>> {
        let t = lua.create_table();
        t.raw_set("Type", "en_US")?;
        t.raw_set("DisplayName", "English")?;
        t.raw_set("Language", "en_US")?;
        t.raw_set("Locale", "en_US")?;
        Ok(t)
    };
    for name in ["GetSupportedLanguages", "GetSupportedSpokenLanguages"] {
        reg.real(lua, &loc, "Locale", name, move |lua, _: MultiValue| {
            let list = lua.create_table();
            list.raw_set(1, language(lua)?)?;
            Ok(list)
        })?;
    }
    for name in ["GetCurrentLanguage", "GetCurrentSpokenLanguage"] {
        reg.real(lua, &loc, "Locale", name, move |lua, _: MultiValue| {
            language(lua)
        })?;
    }
    for name in ["SetCurrentLanguage", "SetCurrentSpokenLanguage"] {
        reg.stub(lua, &loc, "Locale", name, "SIMP-exe-391", Stub::Nil)?;
    }
    // `Locale.LookupLanguage(language, key)`: the one language there is.
    lua.load("function Locale.LookupLanguage(_, key) return Locale.ConvertTextKey(key) end")
        .set_name("=m4_wog")
        .set_environment(g.clone())
        .exec()?;
    simp_mark(inner, "Locale.LookupLanguage", "SIMP-exe-391");

    for (name, items) in [
        ("EndGameTypes", END_GAME_TYPES),
        ("GameStateTypes", GAME_STATES),
    ] {
        if matches!(g.raw_get::<_, LuaValue>(name)?, LuaValue::Nil) {
            let t = lua.create_table();
            for (k, v) in items {
                reg.value(&t, name, k, *v)?;
            }
            g.raw_set(name, t)?;
            reg.scope(name);
        }
    }
    simp_mark(inner, "EndGameTypes", "SIMP-exe-395");
    Ok(())
}

/// `UI.*` and the like of the game menu, the save menu and the end of the game.
fn ui_and_game_menu<'l>(
    core: &Rc<Core>,
    reg: &Reg<'_>,
    inner: &Rc<Inner>,
    lua: &'l Lua,
    g: &Table<'l>,
) -> Result<()> {
    let ui: Table = g.raw_get("UI")?;
    let missing = |name: &str| matches!(ui.raw_get::<_, LuaValue>(name), Ok(LuaValue::Nil));

    // The map of the game is a script (the scripts then look the name up in `GameInfo.MapScripts`).
    reg.stub(lua, &ui, "UI", "GetMapPreview", "SIMP-exe-390", Stub::Nil)?;
    if missing("AreMediumLeadersAllowed") {
        reg.stub(
            lua,
            &ui,
            "UI",
            "AreMediumLeadersAllowed",
            "SIMP-exe-391",
            Stub::Bool(false),
        )?;
    }
    if missing("IsDX9") {
        reg.stub(lua, &ui, "UI", "IsDX9", "SIMP-exe-391", Stub::Bool(false))?;
    }
    if missing("GetCurrentGameState") {
        reg.real(lua, &ui, "UI", "GetCurrentGameState", |_, _: MultiValue| {
            Ok(3)
        })?;
    }

    // The save menu: the host asks the server for a save (SIMP-exe-393).
    // A game of the network saves with `UI.CopyLastAutoSave(name)` (the exe copies the newest autosave), the others
    // with `UI.SaveGame(name)`: both ask the server for a save.
    for fname in ["SaveGame", "CopyLastAutoSave"] {
        let w = Rc::downgrade(core);
        reg.real(lua, &ui, "UI", fname, move |_, args: MultiValue| {
            if let Some(c) = w.upgrade() {
                let name = match args.get(0) {
                    Some(LuaValue::String(s)) => s.to_string_lossy().into_owned(),
                    _ => String::new(),
                };
                if c.host.seat_info().is_host() {
                    c.host.save_game(&name);
                }
            }
            Ok(true)
        })?;
        simp_mark(inner, &format!("UI.{fname}"), "SIMP-exe-393");
    }
    // The list of the save menu is the saves of the server (the host's `saved_games`).
    let w = Rc::downgrade(core);
    reg.real(
        lua,
        &ui,
        "UI",
        "SaveFileList",
        move |_, args: MultiValue| {
            if let (Some(LuaValue::Table(t)), Some(c)) = (args.get(0), w.upgrade()) {
                for (i, s) in c.host.saved_games().iter().enumerate() {
                    t.raw_set(
                        i as i32 + 1,
                        LuaValue::String(LuaString(s.name.clone().into_bytes())),
                    )?;
                }
            }
            Ok(())
        },
    )?;
    simp_mark(inner, "UI.SaveFileList", "SIMP-exe-393");
    let w = Rc::downgrade(core);
    reg.real(
        lua,
        &ui,
        "UI",
        "GetSavedGameModificationTime",
        move |_, args: MultiValue| {
            let name = match args.get(0) {
                Some(LuaValue::String(s)) => s.to_string_lossy().into_owned(),
                _ => String::new(),
            };
            Ok(w.upgrade()
                .and_then(|c| c.host.saved_games().into_iter().find(|s| s.name == name))
                .map(|s| utc_text(s.saved_at_ms))
                .unwrap_or_default())
        },
    )?;
    simp_mark(inner, "UI.GetSavedGameModificationTime", "SIMP-exe-393");
    for name in ["QuickSave", "DeleteSavedGame"] {
        reg.stub(lua, &ui, "UI", name, "SIMP-exe-393", Stub::Nil)?;
    }
    // `PreGame.GetFileHeader(name)` of a save of the server: the turn is known, the rest is "unknown".
    if let LuaValue::Table(pre) = g.raw_get::<_, LuaValue>("PreGame")? {
        let w = Rc::downgrade(core);
        reg.real(
            lua,
            &pre,
            "PreGame",
            "GetFileHeader",
            move |lua, args: MultiValue| {
                let name = match args.get(0) {
                    Some(LuaValue::String(s)) => s.to_string_lossy().into_owned(),
                    _ => String::new(),
                };
                let h = lua.create_table();
                if let Some(s) = w
                    .upgrade()
                    .and_then(|c| c.host.saved_games().into_iter().find(|s| s.name == name))
                {
                    h.raw_set("TurnNumber", s.turn)?;
                }
                for k in [
                    "GameSpeed",
                    "StartEra",
                    "CurrentEra",
                    "Difficulty",
                    "WorldSize",
                    "MapScript",
                    "LeaderName",
                    "CivilizationName",
                ] {
                    h.raw_set(k, "")?;
                }
                h.raw_set("PlayerCivilization", -1)?;
                Ok(h)
            },
        )?;
        simp_mark(inner, "PreGame.GetFileHeader", "SIMP-exe-393");
    }
    // The save window shows the game being played: the exe's `PreGame` takes no player for these, the replica's
    // takes one (and lacks some); a missing player is the active one, a missing getter an answer of the replica.
    lua.load(
        r#"
GameTypes = GameTypes or { GAME_SINGLE_PLAYER = 0, GAME_NETWORK_MULTIPLAYER = 1, GAME_HOTSEAT_MULTIPLAYER = 2 }
local P = PreGame
local function active() return Game.GetActivePlayer() end
local function wrap(name, fallback)
  local f = P[name]
  P[name] = function(...)
    if f then
      local ok, r = pcall(f, ...)
      if ok and r ~= nil then return r end
      if select('#', ...) == 0 then
        ok, r = pcall(f, active())
        if ok and r ~= nil then return r end
      end
    end
    if type(fallback) == "function" then return fallback(...) end
    return fallback
  end
end
wrap("GetCivilization", function() return Players[active()]:GetCivilizationType() end)
-- The replica's pregame table has the civilization of a seat as the lobby knew it; in a running game a seat
-- (an AI above all) answers one shared value there, so every civ icon (turn queue, diplomacy, score lists,
-- city banners: CivIconHookup) was one emblem. The live player knows its own civilization.
-- SIMPLIFIED(SIMP-exe-650): in a running game PreGame.GetCivilization(player) is Players[player]:GetCivilizationType().
do
  local base = P.GetCivilization
  P.GetCivilization = function(pid, ...)
    if type(pid) == "number" and Players and pid >= 0 then
      local p = Players[pid]
      if p then
        local ok, r = pcall(p.GetCivilizationType, p)
        if ok and type(r) == "number" and r >= 0 then return r end
      end
    end
    return base(pid, ...)
  end
end
wrap("GetLeaderName", "")
wrap("GetCivilizationDescription", "")
wrap("GetGameSpeed", function() return Game.GetGameSpeedType() end)
wrap("GetEra", 0)
wrap("GetHandicap", function() return Game.GetHandicapType() end)
wrap("GetWorldSize", function() return Map.GetWorldSize() end)
wrap("GetMapScript", "")
wrap("GetGameType", function() return GameTypes and GameTypes.GAME_NETWORK_MULTIPLAYER or 1 end)
pcall(function()
  -- WorldView.lua's right click: `UI.IsCameraMoving() and not Game.GetAllowRClickMovementWhileScrolling()` (the engine's option, off)
  if not Game.GetAllowRClickMovementWhileScrolling then Game.GetAllowRClickMovementWhileScrolling = function() return false end end
  if not Game.GetElapsedGameTurns then Game.GetElapsedGameTurns = function() return Game.GetGameTurn() end end
  -- the default save name is `leader_<time>`: no character a file name cannot have (the save menu refuses them)
  local old = Game.GetTimeString
  Game.GetTimeString = function()
    local s = old and old() or ("T" .. tostring(Game.GetGameTurn()))
    return (tostring(s):gsub('[:/\\*?"<>|%%]', "-"))
  end
end)
"#,
    )
    .set_name("=m4_wog")
    .set_environment(g.clone())
    .exec()?;
    simp_mark(inner, "PreGame.GetCivilization", "SIMP-exe-393");
    // "Exit to Windows" is leaving for the lobby (SIMP-exe-390).
    let w = Rc::downgrade(core);
    reg.real(lua, &ui, "UI", "ExitGame", move |_, _: MultiValue| {
        if let Some(c) = w.upgrade() {
            c.host.exit_to_lobby();
        }
        Ok(())
    })?;
    simp_mark(inner, "UI.ExitGame", "SIMP-exe-390");

    // Only the host sees the Save button (SIMP-exe-394).
    let modding: Table = g.raw_get("Modding")?;
    let w = Rc::downgrade(core);
    reg.real(
        lua,
        &modding,
        "Modding",
        "AnyActivatedModsContainPropertyValue",
        move |_, args: MultiValue| {
            let name = match args.get(0) {
                Some(LuaValue::String(s)) => s.to_string_lossy().into_owned(),
                _ => String::new(),
            };
            Ok(name == "DisableSaveGameOption"
                && w.upgrade().is_some_and(|c| !c.host.seat_info().is_host()))
        },
    )?;
    simp_mark(
        inner,
        "Modding.AnyActivatedModsContainPropertyValue",
        "SIMP-exe-394",
    );

    // Extended play after a victory does not exist on the server (SIMP-exe-395).
    let net: Table = g.raw_get("Network")?;
    if matches!(
        net.raw_get::<_, LuaValue>("SendExtendedGame")?,
        LuaValue::Nil
    ) {
        reg.stub(
            lua,
            &net,
            "Network",
            "SendExtendedGame",
            "SIMP-exe-395",
            Stub::Nil,
        )?;
    }
    // The in-game options OK compares the quick combat / movement state with the game's and sends the change to
    // the host: our games always run with both off, and the change goes nowhere (SIMP-exe-391).
    if let LuaValue::Table(pre) = g.raw_get::<_, LuaValue>("PreGame")? {
        for name in ["GetQuickCombat", "GetQuickMovement"] {
            if matches!(pre.raw_get::<_, LuaValue>(name)?, LuaValue::Nil) {
                reg.stub(
                    lua,
                    &pre,
                    "PreGame",
                    name,
                    "SIMP-exe-391",
                    Stub::Bool(false),
                )?;
            }
        }
    }
    if matches!(
        net.raw_get::<_, LuaValue>("SendGameOptions")?,
        LuaValue::Nil
    ) {
        reg.stub(
            lua,
            &net,
            "Network",
            "SendGameOptions",
            "SIMP-exe-391",
            Stub::Nil,
        )?;
    }
    Ok(())
}

/// `2026-10-30 10:32` (UTC) of a unix time in milliseconds.
fn utc_text(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil-from-days (H. Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

/// The Lua that asks the replica whether the game is over: -1 when it is not, else `type * 256 + team + 1`, with the
/// `EndGameTypes` of the victory (a time victory when the victory is unknown) and the winning team.
const GAME_OVER_LUA: &str = r#"
-- GAMESTATE_ON 0, OVER 1, EXTENDED 2: a win of the server's game ends in EXTENDED (the others play on), so any
-- state but ON with a winner is the end of the game.
local ON = GameplayGameStateTypes and GameplayGameStateTypes.GAMESTATE_ON or 0
local over = Game and Game.GetGameState and Game.GetGameState() ~= ON
  and Game.GetWinner and Game.GetWinner() >= 0
if not over then return -1 end
local vtype = ""
local v = Game.GetVictory and Game.GetVictory() or -1
if v and v >= 0 and GameInfo.Victories[v] then vtype = GameInfo.Victories[v].Type end
local t = EndGameTypes.Time
if vtype == "VICTORY_SPACE_RACE" then t = EndGameTypes.Technology
elseif vtype == "VICTORY_DOMINATION" then t = EndGameTypes.Domination
elseif vtype == "VICTORY_CULTURAL" then t = EndGameTypes.Culture
elseif vtype == "VICTORY_DIPLOMATIC" then t = EndGameTypes.Diplomatic end
local team = Game.GetWinner and Game.GetWinner() or -1
return t * 256 + team + 1
"#;

impl Core {
    /// The replica loaded a state: when the game is over, `Events.EndGameShow(type, team)` (once). The exe raises it
    /// from `CvGame::showEndGameSequence` when the game core ends the game; the replica runs no turn, so the end of the
    /// game arrives as a state (SIMP-exe-395).
    pub(crate) fn check_game_over(&self, lua: &Lua) {
        let packed = match lua
            .load(GAME_OVER_LUA)
            .set_name("=exe_game_over")
            .eval::<i32>()
        {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(target: "exe.end", "the game-over probe failed: {e}");
                -1
            }
        };
        if packed < 0 {
            self.end_game_shown.set(false);
            return;
        }
        if self.end_game_shown.replace(true) {
            return;
        }
        let (kind, team) = (packed / 256, packed % 256 - 1);
        tracing::info!(target: "exe.end", kind, team, "the game is over: Events.EndGameShow");
        self.fire_events(
            "EndGameShow",
            vec![crate::core::Arg::from(kind), crate::core::Arg::from(team)],
        );
    }
}
